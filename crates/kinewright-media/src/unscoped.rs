//! Compatibility boundary for callers without a source grant.
//! Raw traits refuse; all decoding lives on `SessionMedia`.
use kinewright_core::{
    Analysis, Document, Export, ExportReport, ExportSettings, MediaAsset, MediaError, MediaEvent,
    Playback, PreviewFrame, ProgressSink, RgbaImage, SceneStatus, SilenceStatus, TimeCode,
    TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord, TranscriptStatus,
    VisualAssetResult, WorkingProof,
};
use std::{path::Path, sync::Arc};

use crate::engine::FfmpegMediaEngine;

impl Playback for FfmpegMediaEngine {
    fn set_document(&self, _: Arc<Document>) {
        self.refuse_unscoped();
    }
    fn request_frame(&self, _: TimeCode) {
        self.refuse_unscoped();
    }
    fn frames(&self) -> crossbeam_channel::Receiver<PreviewFrame> {
        crossbeam_channel::never()
    }
    fn events(&self) -> crossbeam_channel::Receiver<MediaEvent> {
        self.unscoped_events()
    }
    fn play(&self, _: TimeCode) {
        self.refuse_unscoped();
    }
    fn pause(&self) {}
    fn seek(&self, _: TimeCode) {
        self.refuse_unscoped();
    }
    fn position(&self) -> TimeCode {
        TimeCode::ZERO
    }
    fn output_peaks(&self) -> [f32; 2] {
        [0.0; 2]
    }
}

impl Analysis for FfmpegMediaEngine {
    fn probe(&self, _: &Path) -> Result<MediaAsset, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn thumbnail_at(&self, _: TimeCode, _: u32) -> Result<RgbaImage, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn thumbnail_for_document(
        &self,
        _: Arc<Document>,
        _: TimeCode,
        _: u32,
    ) -> Result<RgbaImage, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn working_proof_for_document(
        &self,
        _: Arc<Document>,
        _: TimeCode,
    ) -> Result<WorkingProof, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn request_transcription(&self, _: MediaAsset) {}
    fn transcript_status(&self, _: &MediaAsset) -> TranscriptStatus {
        TranscriptStatus::Failed(MediaError::MediaAdmissionRequired.to_string())
    }
    fn timeline_transcript(
        &self,
        _: &Document,
        _: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn request_silence_detection(&self, _: MediaAsset) {}
    fn silence_status(&self, _: &MediaAsset) -> SilenceStatus {
        SilenceStatus::Failed(MediaError::MediaAdmissionRequired.to_string())
    }
    fn timeline_silences(
        &self,
        _: &Document,
        _: Option<std::ops::Range<TimeCode>>,
        _: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn request_scene_detection(&self, _: MediaAsset) {}
    fn scene_status(&self, _: &MediaAsset) -> SceneStatus {
        SceneStatus::Failed(MediaError::MediaAdmissionRequired.to_string())
    }
    fn timeline_scene_changes(
        &self,
        _: &Document,
        _: Option<std::ops::Range<TimeCode>>,
        _: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn request_waveform(&self, _: MediaAsset, _: u64) -> bool {
        false
    }
    fn request_thumbnail(&self, _: MediaAsset, _: TimeCode, _: u32, _: u64) -> bool {
        false
    }
    fn visual_asset_results(&self) -> crossbeam_channel::Receiver<VisualAssetResult> {
        crossbeam_channel::never()
    }
}

impl Export for FfmpegMediaEngine {
    fn export(&self, _: &Path, _: ExportSettings, _: ProgressSink) -> Result<(), MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn export_document_reporting(
        &self,
        _: Arc<Document>,
        _: &Path,
        _: ExportSettings,
        _: ProgressSink,
    ) -> Result<ExportReport, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
}
