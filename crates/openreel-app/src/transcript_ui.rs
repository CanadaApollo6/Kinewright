use std::time::Duration;

use eframe::egui;
use openreel_core::{MediaAsset, MediaEngine, TimelineTranscriptWord, TranscriptStatus};

use crate::{app::OpenReelApp, theme::color};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptScope {
    Asset,
    #[default]
    Timeline,
}

impl OpenReelApp {
    pub(crate) fn transcript_panel(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Transcript")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.transcript_scope,
                        TranscriptScope::Timeline,
                        "Timeline",
                    );
                    ui.selectable_value(
                        &mut self.transcript_scope,
                        TranscriptScope::Asset,
                        "Selected asset",
                    );
                });
                match self.transcript_scope {
                    TranscriptScope::Asset => self.asset_transcript_ui(ui),
                    TranscriptScope::Timeline => self.timeline_transcript_ui(ui),
                }
            });
    }

    fn asset_transcript_ui(&mut self, ui: &mut egui::Ui) {
        let Some(asset) = self.selected_transcript_asset().cloned() else {
            ui.label("Select or import an asset to see its transcript.");
            return;
        };
        ui.small(format!(
            "{} · source {}/{} fps",
            asset.name,
            asset.fps.numerator(),
            asset.fps.denominator()
        ));
        let status = self.media.transcript_status(asset.id);
        match status {
            TranscriptStatus::NotRequested => {
                self.media.request_transcription(asset);
                ui.label("Transcription queued…");
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            TranscriptStatus::Queued => self.running_transcript_label(ui, "Queued…", None),
            TranscriptStatus::Hashing => {
                self.running_transcript_label(ui, "Hashing media…", None);
            }
            TranscriptStatus::DownloadingModel {
                downloaded_bytes,
                total_bytes,
            } => {
                let progress = total_bytes
                    .filter(|total| *total > 0)
                    .map(|total| downloaded_bytes as f32 / total as f32);
                let label = total_bytes.map_or_else(
                    || {
                        format!(
                            "Downloading Whisper model… {} MiB",
                            downloaded_bytes / 1_048_576
                        )
                    },
                    |total| {
                        format!(
                            "Downloading Whisper model… {} / {} MiB",
                            downloaded_bytes / 1_048_576,
                            total / 1_048_576
                        )
                    },
                );
                self.running_transcript_label(ui, &label, progress);
            }
            TranscriptStatus::Transcribing { progress_percent } => {
                self.running_transcript_label(
                    ui,
                    &format!("Transcribing… {progress_percent}%"),
                    Some(f32::from(progress_percent) / 100.0),
                );
            }
            TranscriptStatus::NoSpeech => {
                ui.label("No speech found.");
            }
            TranscriptStatus::Failed(error) => {
                ui.colored_label(
                    color::STATUS_DANGER,
                    format!("Transcription failed: {error}"),
                );
                if ui.button("Retry").clicked() {
                    self.media.request_transcription(asset);
                }
            }
            TranscriptStatus::Ready(transcript) => {
                let mapped = self
                    .media
                    .timeline_transcript(&self.document, None)
                    .unwrap_or_default();
                let selected_clip = self.selected_clip;
                let mut seek = None;
                egui::ScrollArea::vertical()
                    .max_height(130.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for word in &transcript.words {
                                let tooltip = format!(
                                    "source {}..{} frames",
                                    word.source_start.0, word.source_end.0
                                );
                                if ui.small_button(&word.text).on_hover_text(tooltip).clicked() {
                                    seek = mapped
                                        .iter()
                                        .filter(|mapped_word| {
                                            mapped_word.asset == transcript.asset
                                                && mapped_word.source_start == word.source_start
                                        })
                                        .min_by_key(|mapped_word| {
                                            (
                                                mapped_word.clip
                                                    != selected_clip.unwrap_or(mapped_word.clip),
                                                mapped_word.project_start,
                                            )
                                        })
                                        .map(|mapped_word| mapped_word.project_start);
                                }
                            }
                        });
                    });
                if let Some(position) = seek {
                    self.seek_to(position);
                }
            }
        }
    }

    fn timeline_transcript_ui(&mut self, ui: &mut egui::Ui) {
        let words = self
            .media
            .timeline_transcript(&self.document, None)
            .unwrap_or_default();
        let statuses = self
            .document
            .media_pool
            .iter()
            .map(|asset| (asset, self.media.transcript_status(asset.id)))
            .collect::<Vec<_>>();
        if words.is_empty() {
            if statuses.iter().any(|(_, status)| status.is_running()) {
                ui.label("Transcribing…");
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            } else if statuses
                .iter()
                .all(|(_, status)| matches!(status, TranscriptStatus::NoSpeech))
                && !statuses.is_empty()
            {
                ui.label("No speech found.");
            } else if self.document.media_pool.is_empty() {
                ui.label("Add media to the timeline to see its transcript.");
            } else {
                for (asset, status) in statuses {
                    if status == TranscriptStatus::NotRequested {
                        self.media.request_transcription(asset.clone());
                    }
                }
                ui.label("Transcript is not ready yet.");
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            return;
        }

        let mut seek = None;
        egui::ScrollArea::vertical()
            .max_height(130.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for word in &words {
                        if transcript_word_button(ui, word).clicked() {
                            seek = Some(word.project_start);
                        }
                    }
                });
            });
        if let Some(position) = seek {
            self.seek_to(position);
        }
    }

    fn selected_transcript_asset(&self) -> Option<&MediaAsset> {
        self.selected_clip
            .and_then(|clip| self.document.clip(clip))
            .and_then(|clip| self.document.asset(clip.asset))
            .or_else(|| {
                self.selected_asset
                    .and_then(|asset| self.document.asset(asset))
            })
            .or_else(|| self.document.media_pool.first())
    }

    fn running_transcript_label(&self, ui: &mut egui::Ui, label: &str, progress: Option<f32>) {
        if let Some(progress) = progress {
            ui.add(egui::ProgressBar::new(progress.clamp(0.0, 1.0)).text(label));
        } else {
            ui.label(label);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

fn transcript_word_button(ui: &mut egui::Ui, word: &TimelineTranscriptWord) -> egui::Response {
    ui.small_button(&word.text).on_hover_text(format!(
        "project {}..{} frames · source {}..{} frames · clip {}",
        word.project_start.0, word.project_end.0, word.source_start.0, word.source_end.0, word.clip
    ))
}
