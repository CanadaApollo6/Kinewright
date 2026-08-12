use std::{ops::RangeInclusive, time::Duration};

use eframe::egui;
use openreel_core::{
    ClipId, MediaAsset, TimeCode, TimelineTranscriptWord, TranscriptStatus, is_filler_word,
};

use crate::{
    app::OpenReelApp,
    icons::{self, Icon},
    theme::{color, radius, space},
    timeline_ui::format_timecode,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptScope {
    Asset,
    #[default]
    Timeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TranscriptWordIdentity {
    clip: ClipId,
    source_start: TimeCode,
}

impl From<&TimelineTranscriptWord> for TranscriptWordIdentity {
    fn from(word: &TimelineTranscriptWord) -> Self {
        Self {
            clip: word.clip,
            source_start: word.source_start,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptSelection {
    anchor: TranscriptWordIdentity,
    head: TranscriptWordIdentity,
}

impl TranscriptSelection {
    pub(crate) fn single(word: &TimelineTranscriptWord) -> Self {
        let identity = TranscriptWordIdentity::from(word);
        Self {
            anchor: identity,
            head: identity,
        }
    }

    fn extend_to(self, word: &TimelineTranscriptWord) -> Self {
        Self {
            head: TranscriptWordIdentity::from(word),
            ..self
        }
    }

    pub(crate) fn indices(self, words: &[TimelineTranscriptWord]) -> Option<RangeInclusive<usize>> {
        let anchor = words
            .iter()
            .position(|word| TranscriptWordIdentity::from(word) == self.anchor)?;
        let head = words
            .iter()
            .position(|word| TranscriptWordIdentity::from(word) == self.head)?;
        Some(anchor.min(head)..=anchor.max(head))
    }
}

impl OpenReelApp {
    pub(crate) fn transcript_panel(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Transcript")
            .default_open(true)
            .show(ui, |ui| {
                let previous_scope = self.transcript_scope;
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
                if self.transcript_scope != previous_scope {
                    self.transcript_selection = None;
                }
                match self.transcript_scope {
                    TranscriptScope::Asset => self.asset_transcript_ui(ui),
                    TranscriptScope::Timeline => self.timeline_transcript_ui(ui),
                }
            });
    }

    // Download byte counts become an approximate f32 progress bar in this immediate-mode panel.
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
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
        let status = self.analysis.transcript_status(&asset);
        match status {
            TranscriptStatus::NotRequested => {
                self.analysis.request_transcription(asset);
                ui.label("Transcription queued…");
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            TranscriptStatus::Queued => Self::running_transcript_label(ui, "Queued…", None),
            TranscriptStatus::Hashing => {
                Self::running_transcript_label(ui, "Hashing media…", None);
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
                Self::running_transcript_label(ui, &label, progress);
            }
            TranscriptStatus::Transcribing { progress_percent } => {
                Self::running_transcript_label(
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
                    self.analysis.request_transcription(asset);
                }
            }
            TranscriptStatus::Ready(transcript) => {
                let mapped = self
                    .analysis
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

    #[allow(clippy::too_many_lines)]
    fn timeline_transcript_ui(&mut self, ui: &mut egui::Ui) {
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.transcript_selection = None;
        }
        let words = crate::transcript_edit::dedup_linked_timeline_words(
            self.analysis
                .timeline_transcript(&self.document, None)
                .unwrap_or_default(),
        );
        let caption_cues = self.timeline_caption_cues();
        let statuses = self
            .document
            .media_pool
            .iter()
            .map(|asset| (asset, self.analysis.transcript_status(asset)))
            .collect::<Vec<_>>();
        if words.is_empty() {
            self.transcript_selection = None;
            ui.horizontal(|ui| {
                add_captions_button(ui, &caption_cues);
            });
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
                        self.analysis.request_transcription(asset.clone());
                    }
                }
                ui.label("Transcript is not ready yet.");
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            return;
        }

        let mut selected = self
            .transcript_selection
            .and_then(|selection| selection.indices(&words));
        if self.transcript_selection.is_some() && selected.is_none() {
            self.transcript_selection = None;
        }

        let selection_summary = selected.as_ref().map(|range| {
            let selection_words = &words[*range.start()..=*range.end()];
            let start = selection_words
                .iter()
                .map(|word| word.project_start)
                .min()
                .expect("a transcript selection is non-empty");
            let end = selection_words
                .iter()
                .map(|word| word.project_end)
                .max()
                .expect("a transcript selection is non-empty");
            let count = range.end().saturating_sub(*range.start()).saturating_add(1);
            (count, start, end)
        });
        let filler_count = crate::transcript_edit::filler_word_indices(&words).len();
        let mut cut_requested = false;
        let mut remove_fillers_requested = false;
        let mut add_captions_requested = false;
        ui.horizontal(|ui| {
            if let Some((count, start, end)) = selection_summary {
                let noun = if count == 1 { "word" } else { "words" };
                if icons::button(ui, Icon::Delete, &format!("Cut {count} {noun} (Del)")).clicked() {
                    cut_requested = true;
                }
                ui.label(
                    egui::RichText::new(format!("{count} {noun} selected"))
                        .color(color::TEXT_SECONDARY),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{} – {}",
                        format_timecode(start, self.document.fps),
                        format_timecode(end, self.document.fps)
                    ))
                    .monospace()
                    .color(color::TEXT_MUTED),
                );
            }
            if filler_count > 0 {
                let label = filler_count_label(filler_count);
                ui.label(egui::RichText::new(&label).color(color::TEXT_MUTED));
                if icons::button(ui, Icon::Delete, &format!("Remove {label}"))
                    .on_hover_text(format!("Remove {label} in one edit"))
                    .clicked()
                {
                    remove_fillers_requested = true;
                }
            }
            add_captions_requested = add_captions_button(ui, &caption_cues);
        });

        let panel_rect =
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 130.0));
        let panel_hovered = ui.rect_contains_pointer(panel_rect);
        let mut clicked_word = None;
        let mut empty_clicked = false;
        egui::ScrollArea::vertical()
            .max_height(130.0)
            .show(ui, |ui| {
                ui.set_min_size(panel_rect.size());
                let panel_response = ui.interact(
                    egui::Rect::from_min_size(ui.cursor().min, panel_rect.size()),
                    ui.id().with("timeline-transcript-empty"),
                    egui::Sense::click(),
                );
                ui.horizontal_wrapped(|ui| {
                    for (index, word) in words.iter().enumerate() {
                        let is_selected = selected
                            .as_ref()
                            .is_some_and(|range| range.contains(&index));
                        let is_playhead =
                            self.position >= word.project_start && self.position < word.project_end;
                        let response = transcript_word_button(
                            ui,
                            word,
                            is_selected,
                            is_playhead,
                            is_filler_word(&word.text),
                        );
                        if self.playing && selected.is_none() && !panel_hovered && is_playhead {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                        if response.clicked() {
                            clicked_word = Some((index, ui.input(|input| input.modifiers.shift)));
                        }
                    }
                });
                empty_clicked = panel_response.clicked();
            });

        if let Some((index, extend)) = clicked_word {
            let word = &words[index];
            self.seek_to(word.project_start);
            self.transcript_selection = Some(if extend {
                self.transcript_selection.map_or_else(
                    || TranscriptSelection::single(word),
                    |selection| selection.extend_to(word),
                )
            } else {
                TranscriptSelection::single(word)
            });
            selected = self
                .transcript_selection
                .and_then(|selection| selection.indices(&words));
        } else if empty_clicked {
            self.transcript_selection = None;
            selected = None;
        }

        if cut_requested && selected.is_some() {
            self.cut_selected_transcript_words();
        }
        if remove_fillers_requested {
            self.remove_filler_words();
        }
        if add_captions_requested {
            self.add_captions();
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

    fn running_transcript_label(ui: &mut egui::Ui, label: &str, progress: Option<f32>) {
        if let Some(progress) = progress {
            ui.add(egui::ProgressBar::new(progress.clamp(0.0, 1.0)).text(label));
        } else {
            ui.label(label);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

fn add_captions_button(
    ui: &mut egui::Ui,
    cues: &Result<Vec<openreel_core::CaptionCue>, String>,
) -> bool {
    let enabled = cues.is_ok();
    let disabled_reason = cues.as_ref().err().map_or("", String::as_str);
    ui.add_enabled_ui(enabled, |ui| icons::button(ui, Icon::Add, "Add captions"))
        .inner
        .on_disabled_hover_text(disabled_reason)
        .clicked()
}

fn transcript_word_button(
    ui: &mut egui::Ui,
    word: &TimelineTranscriptWord,
    selected: bool,
    playhead: bool,
    filler: bool,
) -> egui::Response {
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(space::HALF, space::HALF);
            ui.spacing_mut().interact_size = egui::Vec2::ZERO;
            let idle_fill = if selected {
                color::ACCENT_28
            } else {
                color::PANEL
            };
            let hover_fill = if selected {
                color::ACCENT_28
            } else {
                color::SURFACE_ACTIVE
            };
            ui.style_mut().visuals.widgets.inactive.bg_fill = idle_fill;
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = idle_fill;
            ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.active.bg_fill = idle_fill;
            ui.style_mut().visuals.widgets.active.weak_bg_fill = idle_fill;
            ui.style_mut().visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.hovered.bg_fill = hover_fill;
            ui.style_mut().visuals.widgets.hovered.weak_bg_fill = hover_fill;
            ui.style_mut().visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
            let text = if playhead {
                egui::RichText::new(&word.text).color(color::ACCENT)
            } else if selected {
                egui::RichText::new(&word.text).color(color::TEXT_PRIMARY)
            } else if filler {
                egui::RichText::new(&word.text).color(color::TEXT_MUTED)
            } else {
                egui::RichText::new(&word.text)
            };
            ui.add(
                egui::Button::new(text)
                    .fill(idle_fill)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(radius::XS)
                    .min_size(egui::Vec2::ZERO),
            )
        })
        .inner;
    if filler && !selected && !playhead {
        let y = response.rect.bottom() - 1.0;
        let left = egui::pos2(response.rect.left() + space::HALF, y);
        let right = egui::pos2(response.rect.right() - space::HALF, y);
        ui.painter()
            .line_segment([left, right], egui::Stroke::new(1.0, color::BORDER_STRONG));
    }
    response.on_hover_text(format!(
        "project {}..{} frames · source {}..{} frames · clip {}",
        word.project_start.0, word.project_end.0, word.source_start.0, word.source_end.0, word.clip
    ))
}

fn filler_count_label(count: usize) -> String {
    let noun = if count == 1 { "filler" } else { "fillers" };
    format!("{count} {noun}")
}

#[cfg(test)]
mod tests {
    use openreel_core::{AssetId, TrackId};

    use super::*;

    fn word(clip: u64, source_start: i64) -> TimelineTranscriptWord {
        TimelineTranscriptWord {
            text: format!("word-{source_start}"),
            asset: AssetId(clip),
            track: TrackId(1),
            clip: ClipId(clip),
            source_start: TimeCode(source_start),
            source_end: TimeCode(source_start + 5),
            project_start: TimeCode(source_start),
            project_end: TimeCode(source_start + 5),
        }
    }

    #[test]
    fn selection_anchor_and_head_resolve_by_clip_and_source_identity() {
        let original = [word(1, 10), word(1, 20)];
        let selection = TranscriptSelection::single(&original[0]).extend_to(&original[1]);
        let rerendered = [word(2, 0), original[0].clone(), original[1].clone()];

        assert_eq!(selection.indices(&rerendered), Some(1..=2));
    }

    #[test]
    fn filler_count_labels_use_singular_and_plural() {
        assert_eq!(filler_count_label(1), "1 filler");
        assert_eq!(filler_count_label(2), "2 fillers");
    }
}
